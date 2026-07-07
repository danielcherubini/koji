use sysinfo::System;

use super::amd::normalize_amd_uuid;
use super::nvidia::parse_nvidia_smi_csv_line;
use super::system::{
    aggregate_utilization_mean, aggregate_vram_sum, assign_position_ids, collect_system_metrics,
    collect_system_metrics_with,
};
use super::types::*;
use super::vram::VramInfo;

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
    // index,name,uuid,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed
    let line = "0, GeForce RTX 4090, GPU-abc123, 45, 4096, 8192, 62, 150, 70";
    let stats = parse_nvidia_smi_csv_line(line);
    assert!(stats.is_some());
    let stats = stats.unwrap();
    assert_eq!(stats.device_id, "nvidia0");
    assert_eq!(stats.vendor, GpuVendor::Nvidia);
    assert_eq!(stats.name, "GeForce RTX 4090");
    assert_eq!(stats.uuid, Some("GPU-abc123".to_string()));
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
    let line = "12, GeForce RTX 4090, GPU-def456, 88, 2048, 24576, 75, 300, 85";
    let stats = parse_nvidia_smi_csv_line(line);
    assert!(stats.is_some());
    let stats = stats.unwrap();
    assert_eq!(stats.device_id, "nvidia12");
    assert_eq!(stats.name, "GeForce RTX 4090");
    assert_eq!(stats.uuid, Some("GPU-def456".to_string()));
    assert_eq!(stats.utilization_pct, Some(88));
}

#[test]
fn test_nvidia_device_id_format() {
    let line = "0, GeForce RTX 4090, GPU-xyz, 10, 100, 200, 30, 50, 60";
    let stats = parse_nvidia_smi_csv_line(line).unwrap();
    assert_eq!(stats.device_id, "nvidia0");
    assert_eq!(stats.name, "GeForce RTX 4090");
    assert_eq!(stats.uuid, Some("GPU-xyz".to_string()));

    let line = "12, GeForce RTX 4090, GPU-uvw, 10, 100, 200, 30, 50, 60";
    let stats = parse_nvidia_smi_csv_line(line).unwrap();
    assert_eq!(stats.device_id, "nvidia12");
    assert_eq!(stats.name, "GeForce RTX 4090");
    assert_eq!(stats.uuid, Some("GPU-uvw".to_string()));
}

#[test]
fn test_parse_nvidia_smi_csv_malformed() {
    // Too few fields
    assert!(parse_nvidia_smi_csv_line("0, 45").is_none());
    // Empty line
    assert!(parse_nvidia_smi_csv_line("").is_none());
    // Non-numeric fields
    assert!(parse_nvidia_smi_csv_line("abc, name, uuid, 45, 100, 200, 30, 50, 60").is_none());
    // Extra fields (10 instead of 9)
    assert!(parse_nvidia_smi_csv_line("0, name, uuid, 45, 100, 200, 30, 50, 60, 99").is_none());
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
        vendor: GpuVendor::Nvidia,
        name: "Test GPU".to_string(),
        utilization_pct: util,
        vram: None,
        temperature_c: None,
        power_w: None,
        fan_pct: None,
        pci_bus: None,
        uuid: None,
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
        vendor: GpuVendor::Nvidia,
        name: "Test GPU".to_string(),
        utilization_pct: None,
        vram,
        temperature_c: None,
        power_w: None,
        fan_pct: None,
        pci_bus: None,
        uuid: None,
    }
}

// ── assign_position_ids tests ──────────────────────────────────────

#[test]
fn test_assign_position_ids_sorts_and_assigns() {
    let mut gpus = vec![
        GpuDeviceStats {
            device_id: "nvidia10".to_string(),
            vendor: GpuVendor::Nvidia,
            name: "RTX 4090".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: None,
        },
        GpuDeviceStats {
            device_id: "nvidia2".to_string(),
            vendor: GpuVendor::Nvidia,
            name: "RTX 3090".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: None,
        },
    ];
    assign_position_ids(&mut gpus);
    // nvidia2 should come before nvidia10
    assert_eq!(gpus.len(), 2);
    assert_eq!(gpus[0].device_id, "GPU0");
    assert_eq!(gpus[0].vendor, GpuVendor::Nvidia);
    assert_eq!(gpus[0].name, "RTX 3090");
    assert_eq!(gpus[1].device_id, "GPU1");
    assert_eq!(gpus[1].vendor, GpuVendor::Nvidia);
    assert_eq!(gpus[1].name, "RTX 4090");
}

#[test]
fn test_assign_position_ids_multiple_per_vendor() {
    let mut gpus = vec![
        GpuDeviceStats {
            device_id: "nvidia1".to_string(),
            vendor: GpuVendor::Nvidia,
            name: "RTX 4090".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: None,
        },
        GpuDeviceStats {
            device_id: "amd0".to_string(),
            vendor: GpuVendor::Amd,
            name: "Radeon RX 7900".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: None,
        },
        GpuDeviceStats {
            device_id: "nvidia0".to_string(),
            vendor: GpuVendor::Nvidia,
            name: "RTX 3090".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: None,
        },
        GpuDeviceStats {
            device_id: "amd1".to_string(),
            vendor: GpuVendor::Amd,
            name: "Radeon RX 6900".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: None,
        },
    ];
    assign_position_ids(&mut gpus);
    // Expected order: amd0, amd1, nvidia0, nvidia1
    assert_eq!(gpus.len(), 4);
    assert_eq!(gpus[0].device_id, "GPU0");
    assert_eq!(gpus[0].vendor, GpuVendor::Amd);
    assert_eq!(gpus[0].name, "Radeon RX 7900");
    assert_eq!(gpus[1].device_id, "GPU1");
    assert_eq!(gpus[1].vendor, GpuVendor::Amd);
    assert_eq!(gpus[1].name, "Radeon RX 6900");
    assert_eq!(gpus[2].device_id, "GPU2");
    assert_eq!(gpus[2].vendor, GpuVendor::Nvidia);
    assert_eq!(gpus[2].name, "RTX 3090");
    assert_eq!(gpus[3].device_id, "GPU3");
    assert_eq!(gpus[3].vendor, GpuVendor::Nvidia);
    assert_eq!(gpus[3].name, "RTX 4090");
}

#[test]
fn test_assign_position_ids_empty() {
    let mut gpus: Vec<GpuDeviceStats> = vec![];
    assign_position_ids(&mut gpus);
    assert!(gpus.is_empty());
}

#[test]
fn test_parse_nvidia_smi_csv_empty_uuid() {
    // Some older nvidia-smi versions return empty UUID field
    let line = "0, GeForce RTX 4090, , 45, 4096, 8192, 62, 150, 70";
    let stats = parse_nvidia_smi_csv_line(line);
    assert!(stats.is_some());
    let stats = stats.unwrap();
    assert_eq!(stats.uuid, None);
    assert_eq!(stats.device_id, "nvidia0");
    assert_eq!(stats.utilization_pct, Some(45));
}

// ── normalize_amd_uuid tests ─────────────────────────────────────

#[test]
fn test_normalize_amd_uuid_0x_prefix() {
    // rocm-smi format → rocminfo format
    assert_eq!(
        normalize_amd_uuid("0xb3780db0a262809e"),
        "GPU-b3780db0a262809e"
    );
}

#[test]
fn test_normalize_amd_uuid_already_gpu_prefix() {
    // Already in rocminfo format — pass through unchanged
    assert_eq!(
        normalize_amd_uuid("GPU-b3780db0a262809e"),
        "GPU-b3780db0a262809e"
    );
}

#[test]
fn test_normalize_amd_uuid_unknown_format() {
    // Unknown format — pass through unchanged
    assert_eq!(normalize_amd_uuid("some-weird-id"), "some-weird-id");
}
